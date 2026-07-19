use super::*;
use aida_core::models::Relationship;

// trace:STORY-48 | ai:claude
fn lease(scope: &str, id: &str) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_lowercase(),
        owner: "tester".into(),
        worktree_path: std::path::PathBuf::from(format!("/tmp/{}", id)),
        branch: format!("br-{}", id),
        started_at: chrono::Utc::now(),
        hostname: "test".into(),
        role: None,
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
    }
}

/// AIDA's parent-edge convention: a child stores `rel_type: Child`
/// pointing at its parent (display reads "X is child of Y"). So to
/// model "this requirement has these parents" in fixtures, we emit
/// `Child` edges from `r` to each parent UUID.
// trace:STORY-48 | ai:claude
fn req_with_parents(spec_id: &str, parents: &[Uuid]) -> Requirement {
    let mut r = Requirement::new(format!("Title for {}", spec_id), "".into());
    r.spec_id = Some(spec_id.into());
    r.relationships = parents
        .iter()
        .map(|pid| Relationship {
            rel_type: RelationshipType::Child,
            target_id: *pid,
            created_at: Some(chrono::Utc::now()),
            created_by: None,
        })
        .collect();
    r
}

/// Direct spec-id ownership: lease scope == target spec id.
// trace:STORY-48 | ai:claude
#[test]
fn lease_owns_direct_spec_match() {
    let target = req_with_parents("STORY-48", &[]);
    let mut store = RequirementsStore::new();
    store.requirements.push(target.clone());
    let leases = vec![lease("STORY-48", "abc123")];
    // BUG-637: this test exercises ancestry/scope matching, not liveness —
    // treat every lease as live so the synthetic worktree paths don't read stale.
    let owner = lease_owning_spec(
        &leases,
        None,
        target.id,
        target.spec_id.as_deref(),
        &store,
        |_| true,
    );
    assert!(owner.is_some());
    assert_eq!(owner.unwrap().scope, "STORY-48");
}

/// EPIC-scope ownership: lease.scope is the parent of target.
// trace:STORY-48 | ai:claude
#[test]
fn lease_owns_via_parent_chain() {
    let epic = req_with_parents("EPIC-20", &[]);
    let story = req_with_parents("STORY-48", &[epic.id]);
    let mut store = RequirementsStore::new();
    store.requirements.push(epic.clone());
    store.requirements.push(story.clone());
    let leases = vec![lease("EPIC-20", "epic")];
    let owner = lease_owning_spec(
        &leases,
        None,
        story.id,
        story.spec_id.as_deref(),
        &store,
        |_| true,
    );
    assert!(
        owner.is_some(),
        "EPIC-scope lease should own descendant story"
    );
    assert_eq!(owner.unwrap().scope, "EPIC-20");
}

/// Self-lease must be skipped — a session can edit specs in its own
/// scope without a warning.
// trace:STORY-48 | ai:claude
#[test]
fn lease_owning_skips_self() {
    let target = req_with_parents("STORY-48", &[]);
    let mut store = RequirementsStore::new();
    store.requirements.push(target.clone());
    let mine = lease("STORY-48", "self");
    let leases = vec![mine.clone()];
    let owner = lease_owning_spec(
        &leases,
        Some(&mine),
        target.id,
        target.spec_id.as_deref(),
        &store,
        |_| true,
    );
    assert!(owner.is_none(), "should not flag the caller's own lease");
}

/// BUG-54: a session whose scope is an EPIC must be allowed to edit
/// children of that EPIC from inside the worktree. Direct-spec-match
/// covers `aida edit EPIC-X` from the EPIC-X session; this exercises
/// the parent-chain case (`aida edit <child-of-EPIC-X>`), which is
/// the actual flow that triggered the in-session enforcement bug.
// trace:BUG-54 | ai:claude
#[test]
fn lease_owning_skips_self_via_parent_chain() {
    let epic = req_with_parents("EPIC-20", &[]);
    let story = req_with_parents("STORY-55", &[epic.id]);
    let mut store = RequirementsStore::new();
    store.requirements.push(epic.clone());
    store.requirements.push(story.clone());
    let mine = lease("EPIC-20", "ownsepic");
    let leases = vec![mine.clone()];
    let owner = lease_owning_spec(
        &leases,
        Some(&mine),
        story.id,
        story.spec_id.as_deref(),
        &store,
        |_| true,
    );
    assert!(
        owner.is_none(),
        "owner-of-EPIC-X session must be allowed to edit children of EPIC-X"
    );
}

/// Path-glob / free-form scopes that don't resolve to a spec id are
/// treated as non-enforced.
// trace:STORY-48 | ai:claude
#[test]
fn lease_owning_ignores_unresolved_scopes() {
    let target = req_with_parents("STORY-48", &[]);
    let mut store = RequirementsStore::new();
    store.requirements.push(target.clone());
    let leases = vec![lease("src/scaffolding/**", "glob")];
    let owner = lease_owning_spec(
        &leases,
        None,
        target.id,
        target.spec_id.as_deref(),
        &store,
        |_| true,
    );
    assert!(owner.is_none());
}

/// Cycle in parent edges must not infinite-loop the ancestor walk.
// trace:STORY-48 | ai:claude
#[test]
fn lease_owning_handles_parent_cycle() {
    let mut a = req_with_parents("FR-A", &[]);
    let mut b = req_with_parents("FR-B", &[]);
    // Cycle uses `Child` edges (the climb-toward-root direction in
    // AIDA's storage convention). Each side points at the other as
    // its "parent" — pathological, but lease_owning_spec must
    // terminate even so.
    a.relationships = vec![Relationship {
        rel_type: RelationshipType::Child,
        target_id: b.id,
        created_at: Some(chrono::Utc::now()),
        created_by: None,
    }];
    b.relationships = vec![Relationship {
        rel_type: RelationshipType::Child,
        target_id: a.id,
        created_at: Some(chrono::Utc::now()),
        created_by: None,
    }];
    let mut store = RequirementsStore::new();
    store.requirements.push(a.clone());
    store.requirements.push(b.clone());
    // No lease covers either; the call must terminate.
    let leases: Vec<SessionLease> = vec![];
    let owner = lease_owning_spec(&leases, None, a.id, a.spec_id.as_deref(), &store, |_| true);
    assert!(owner.is_none());
}

/// BUG-98: list_leases scans the on-disk lease toml files and
/// returns one entry per file. This is the count `aida session list`
/// uses to decide whether to render the leases-hint footer. The
/// shape exercised here matches the BUG-98 repro (multiple leases
/// from concurrent worktrees on the project).
// trace:BUG-98 | ai:claude
#[test]
fn list_leases_counts_active_lease_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project_root = tmp.path().to_path_buf();
    let dir = leases_dir(&project_root);
    std::fs::create_dir_all(&dir).unwrap();
    // Empty directory → 0.
    assert_eq!(list_leases(&project_root).len(), 0);

    // Drop three valid lease toml files and one bogus non-toml — the
    // count should be 3.
    for (idx, scope) in ["EPIC-20", "PR-19", "EPIC-21"].iter().enumerate() {
        let id = format!("019e0000000{:01}", idx);
        let toml = format!(
                "id = \"{id}\"\nscope = \"{scope}\"\nslug = \"{slug}\"\nowner = \"t\"\n\
                 worktree_path = \"/tmp/{id}\"\nbranch = \"br\"\nstarted_at = \"2026-05-14T00:00:00Z\"\n\
                 hostname = \"h\"\n",
                id = id,
                scope = scope,
                slug = scope.to_lowercase(),
            );
        std::fs::write(dir.join(format!("{}.toml", id)), toml).unwrap();
    }
    std::fs::write(dir.join("README.txt"), "ignored").unwrap();
    let leases = list_leases(&project_root);
    assert_eq!(
        leases.len(),
        3,
        "expected 3 active leases, got {} ({:?})",
        leases.len(),
        leases.iter().map(|l| l.scope.as_str()).collect::<Vec<_>>()
    );
    // Sorted by started_at (all equal here) — make sure no scope is
    // dropped silently.
    let scopes: Vec<&str> = leases.iter().map(|l| l.scope.as_str()).collect();
    assert!(scopes.contains(&"EPIC-20"));
    assert!(scopes.contains(&"PR-19"));
    assert!(scopes.contains(&"EPIC-21"));
}

/// BUG-479: `child_side_work_exists` is the pure decision — ANY of the three
/// child-side signals means real work was done before the non-zero exit, so
// the spec must stay shelved (don't restore). trace:BUG-479 | ai:claude
#[test]
fn bug479_child_side_work_exists_is_any_signal() {
    assert!(!child_side_work_exists(false, false, false));
    assert!(child_side_work_exists(true, false, false)); // lease alone
    assert!(child_side_work_exists(false, true, false)); // worktree alone
    assert!(child_side_work_exists(false, false, true)); // commits alone
    assert!(child_side_work_exists(true, true, true));
}

/// BUG-479: the probe (and therefore the restore guard) must report
/// child-side work when a lease scoped to the spec exists, and report none
/// when no lease matches. A lease present ⇒ the implementer child acquired a
/// lease + worktree before exiting non-zero, so
/// `restore_phase1_status_on_lease_failure` would bail (leave it shelved)
/// rather than reset; no lease ⇒ genuinely no child-side work, restore is
// safe. trace:BUG-479 | ai:claude
#[test]
fn bug479_probe_reports_lease_scoped_to_spec_and_skips_unrelated() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project_root = tmp.path().to_path_buf();
    let dir = leases_dir(&project_root);
    std::fs::create_dir_all(&dir).unwrap();

    // No leases yet ⇒ genuinely no child-side work ⇒ restore would proceed.
    let (has_lease, has_worktree, has_commits) =
        probe_child_side_work_for_spec(&project_root, "TASK-479");
    assert!(!child_side_work_exists(
        has_lease,
        has_worktree,
        has_commits
    ));

    // A lease scoped to a DIFFERENT spec must not count.
    let other = "id = \"019e0000aaaa\"\nscope = \"TASK-999\"\nslug = \"task-999\"\n\
             owner = \"t\"\nworktree_path = \"/tmp/does-not-exist-479\"\nbranch = \"br\"\n\
             started_at = \"2026-05-14T00:00:00Z\"\nhostname = \"h\"\n";
    std::fs::write(dir.join("019e0000aaaa.toml"), other).unwrap();
    let (has_lease, has_worktree, has_commits) =
        probe_child_side_work_for_spec(&project_root, "TASK-479");
    assert!(
        !child_side_work_exists(has_lease, has_worktree, has_commits),
        "an unrelated-scope lease must not be read as child-side work for TASK-479"
    );

    // A lease scoped to the SPEC (case-insensitively) ⇒ child-side work
    // exists ⇒ the restore guard must bail and leave it shelved.
    let mine = "id = \"019e0000bbbb\"\nscope = \"task-479\"\nslug = \"task-479\"\n\
             owner = \"t\"\nworktree_path = \"/tmp/does-not-exist-479\"\nbranch = \"br\"\n\
             started_at = \"2026-05-14T00:00:00Z\"\nhostname = \"h\"\n";
    std::fs::write(dir.join("019e0000bbbb.toml"), mine).unwrap();
    let (has_lease, has_worktree, has_commits) =
        probe_child_side_work_for_spec(&project_root, "TASK-479");
    assert!(has_lease, "a lease scoped to the spec must be detected");
    assert!(
        child_side_work_exists(has_lease, has_worktree, has_commits),
        "a lease scoped to the spec must count as child-side work (leave shelved)"
    );

    // And the restore path itself must NOT error and must short-circuit:
    // with child-side work present it returns Ok without needing a store.
    restore_phase1_status_on_lease_failure(
        &project_root,
        "TASK-479",
        &aida_core::RequirementStatus::Approved,
    )
    .expect("guard should short-circuit cleanly when child-side work exists");
}

/// BUG-483: `peer_lease_sharing_worktree` is the pure gate that decides
/// whether `aida session end` must SKIP its `git worktree remove --force`.
/// Two leases sharing one `worktree_path` ⇒ ending one finds the peer and
/// leaves the dir standing; a sole lease on a worktree ⇒ no peer ⇒ removal
/// proceeds as before. We write lease .toml fixtures and read them back
/// through `list_leases` so the real on-disk shape is exercised.
// trace:BUG-483 | ai:claude
#[test]
fn bug483_peer_lease_sharing_worktree_blocks_removal() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project_root = tmp.path().to_path_buf();
    let dir = leases_dir(&project_root);
    std::fs::create_dir_all(&dir).unwrap();

    // Two `aida agent new` sessions sharing ONE worktree (BUG-416 scenario).
    let shared_wt = tmp.path().join("shared-worktree");
    std::fs::create_dir_all(&shared_wt).unwrap();
    let shared_wt_str = shared_wt.to_str().unwrap();
    // A third, sole-occupant lease on its own distinct worktree.
    let solo_wt = tmp.path().join("solo-worktree");
    std::fs::create_dir_all(&solo_wt).unwrap();
    let solo_wt_str = solo_wt.to_str().unwrap();

    let write_lease = |id: &str, scope: &str, wt: &str| {
        // worktree_path goes in a TOML *literal* (single-quoted) string: on
        // Windows `wt` is a backslash path (C:\...\shared-worktree), and a
        // double-quoted basic string would read each `\` as an escape
        // sequence → invalid TOML → the lease silently fails to parse and is
        // dropped from list_leases. Production serializes via
        // toml::to_string_pretty (which escapes the backslashes); the test
        // mirrors that round-trip safely with a literal string.
        // trace:BUG-483 | ai:claude
        let toml = format!(
            "id = \"{id}\"\nscope = \"{scope}\"\nslug = \"{slug}\"\nowner = \"t\"\n\
                 worktree_path = '{wt}'\nbranch = \"br\"\nstarted_at = \"2026-05-14T00:00:00Z\"\n\
                 hostname = \"h\"\n",
            id = id,
            scope = scope,
            slug = scope.to_lowercase(),
            wt = wt,
        );
        std::fs::write(dir.join(format!("{id}.toml")), toml).unwrap();
    };
    write_lease("019e0000a001", "TASK-483", shared_wt_str);
    write_lease("019e0000a002", "STORY-483", shared_wt_str);
    write_lease("019e0000a003", "BUG-483", solo_wt_str);

    let leases = list_leases(&project_root);
    assert_eq!(leases.len(), 3, "expected the three fixture leases");

    // Ending the first shared-worktree lease must find the second as a
    // peer ⇒ removal is SKIPPED (the dir is left in place for the peer).
    let peer = peer_lease_sharing_worktree(&leases, "019e0000a001", &shared_wt)
        .expect("a peer lease shares the worktree ⇒ removal must be skipped");
    assert_eq!(
        peer.id, "019e0000a002",
        "the OTHER shared-worktree lease must be reported as the peer"
    );

    // Symmetrically, ending the peer finds the first.
    let peer2 = peer_lease_sharing_worktree(&leases, "019e0000a002", &shared_wt)
        .expect("the reciprocal peer must also be detected");
    assert_eq!(peer2.id, "019e0000a001");

    // The sole-occupant lease has no peer ⇒ removal proceeds as today.
    assert!(
        peer_lease_sharing_worktree(&leases, "019e0000a003", &solo_wt).is_none(),
        "a sole lease on a worktree must not see a phantom peer ⇒ removal proceeds"
    );

    // A lease must never count itself as its own peer.
    assert!(
        peer_lease_sharing_worktree(&leases, "019e0000a001", &shared_wt)
            .map(|p| p.id != "019e0000a001")
            .unwrap_or(false),
        "the ending lease must be filtered out of the peer search by id"
    );

    // An empty `worktree_path` (advisory MCP claim lock, TASK-474) is never
    // a worktree peer even if another such lock also has an empty path.
    write_lease("019e0000a004", "ADV-1", "");
    write_lease("019e0000a005", "ADV-2", "");
    let leases = list_leases(&project_root);
    assert!(
        peer_lease_sharing_worktree(&leases, "019e0000a004", std::path::Path::new("")).is_none(),
        "empty worktree_path locks must not match each other as worktree peers"
    );
}

/// BUG-416: `worktree_occupant` is the detection core for the
/// detect-and-auto-isolate gate. It must (1) find a LIVE lease on the
/// target worktree, (2) ignore a lease on a DIFFERENT worktree, and
/// (3) treat a dead/auto-released lease's worktree as free — driven by
/// the injected liveness predicate so the pure core is testable without
// poking real PIDs. trace:BUG-416 | ai:claude
#[test]
fn worktree_occupant_finds_only_live_same_worktree_lease() {
    fn lease(scope: &str, wt: &str, pid: u32) -> SessionLease {
        SessionLease {
            id: format!("lease-{scope}"),
            scope: scope.into(),
            slug: scope.to_lowercase(),
            owner: "t".into(),
            worktree_path: std::path::PathBuf::from(wt),
            branch: "br".into(),
            started_at: chrono::Utc::now(),
            hostname: "h".into(),
            role: None,
            creator_pid: Some(pid),
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
        }
    }
    let leases = vec![
        lease("EPIC-1", "/tmp/wt-a", 100), // dead (per predicate below)
        lease("EPIC-2", "/tmp/wt-a", 200), // live, on the target worktree
        lease("EPIC-3", "/tmp/wt-b", 300), // live, but different worktree
    ];
    // Liveness predicate: every pid is alive except 100.
    let is_live = |l: &SessionLease| l.creator_pid != Some(100);

    // A live lease occupies /tmp/wt-a → detected (the EPIC-2 one, not the
    // dead EPIC-1 sharing the path).
    let occ =
        worktree_occupant(std::path::Path::new("/tmp/wt-a"), &leases, is_live).expect("occupied");
    assert_eq!(occ.scope, "EPIC-2");

    // A worktree with no lease at all → free.
    assert!(worktree_occupant(std::path::Path::new("/tmp/wt-c"), &leases, is_live).is_none());

    // If the only lease on a worktree is dead, the worktree is free to take.
    let only_dead = vec![lease("EPIC-9", "/tmp/wt-d", 100)];
    assert!(
        worktree_occupant(std::path::Path::new("/tmp/wt-d"), &only_dead, is_live).is_none(),
        "a dead lease's worktree must read as free for auto-isolate"
    );
}
