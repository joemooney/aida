use super::*;
use aida_core::models::Relationship;

fn lease_for(scope: &str) -> SessionLease {
    SessionLease {
        id: "selflease0001".into(),
        scope: scope.to_string(),
        slug: scope.to_lowercase(),
        owner: "tester".into(),
        worktree_path: std::path::PathBuf::from("/tmp/x"),
        branch: "br".into(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
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

fn child_of(parent_uuid: Uuid, spec_id: &str) -> Requirement {
    let mut r = Requirement::new(format!("Title for {}", spec_id), "".into());
    r.spec_id = Some(spec_id.into());
    r.relationships = vec![Relationship {
        rel_type: RelationshipType::Child,
        target_id: parent_uuid,
        created_at: Some(chrono::Utc::now()),
        created_by: None,
    }];
    r.status = RequirementStatus::Approved;
    r.priority = RequirementPriority::Medium;
    r
}

fn scope_root(spec_id: &str, child_uuids: &[Uuid]) -> Requirement {
    let mut r = Requirement::new(format!("Title for {}", spec_id), "".into());
    r.spec_id = Some(spec_id.into());
    r.relationships = child_uuids
        .iter()
        .map(|cid| Relationship {
            rel_type: RelationshipType::Parent,
            target_id: *cid,
            created_at: Some(chrono::Utc::now()),
            created_by: None,
        })
        .collect();
    r
}

/// Highest-priority approved child wins.
// trace:STORY-63 | ai:claude
#[test]
fn picks_highest_priority_approved_child() {
    let mut a = child_of(Uuid::nil(), "STORY-1");
    let mut b = child_of(Uuid::nil(), "STORY-2");
    let mut c = child_of(Uuid::nil(), "STORY-3");
    a.priority = RequirementPriority::Low;
    b.priority = RequirementPriority::High;
    c.priority = RequirementPriority::Medium;
    let epic = scope_root("EPIC-20", &[a.id, b.id, c.id]);
    // Patch ancestors so child rels point at the EPIC's actual id.
    for child in [&mut a, &mut b, &mut c] {
        child.relationships[0].target_id = epic.id;
    }
    let mut store = RequirementsStore::new();
    store.requirements.extend([epic.clone(), a, b.clone(), c]);
    let lease = lease_for("EPIC-20");
    let res = scope_fallback_pick(&store, &lease, None).expect("expected a pick");
    assert_eq!(res.pick.spec_id.as_deref(), Some("STORY-2"));
    assert_eq!(res.approved_count, 3);
}

/// Created_at breaks ties at equal priority — older wins.
// trace:STORY-63 | ai:claude
#[test]
fn ties_break_on_created_at_oldest_first() {
    let mut older = child_of(Uuid::nil(), "STORY-A");
    let mut newer = child_of(Uuid::nil(), "STORY-B");
    older.priority = RequirementPriority::High;
    newer.priority = RequirementPriority::High;
    let now = chrono::Utc::now();
    older.created_at = now - chrono::Duration::hours(2);
    newer.created_at = now;
    let epic = scope_root("EPIC-20", &[older.id, newer.id]);
    for child in [&mut older, &mut newer] {
        child.relationships[0].target_id = epic.id;
    }
    let mut store = RequirementsStore::new();
    store.requirements.extend([epic.clone(), older, newer]);
    let lease = lease_for("EPIC-20");
    let res = scope_fallback_pick(&store, &lease, None).expect("expected a pick");
    assert_eq!(res.pick.spec_id.as_deref(), Some("STORY-A"));
}

/// Any sibling InProgress → no pick (don't run two children in
/// parallel under the same EPIC).
// trace:STORY-63 | ai:claude
#[test]
fn skips_when_sibling_in_progress() {
    let mut active = child_of(Uuid::nil(), "STORY-ACTIVE");
    let mut waiting = child_of(Uuid::nil(), "STORY-WAITING");
    active.status = RequirementStatus::InProgress;
    waiting.priority = RequirementPriority::High;
    let epic = scope_root("EPIC-20", &[active.id, waiting.id]);
    for child in [&mut active, &mut waiting] {
        child.relationships[0].target_id = epic.id;
    }
    let mut store = RequirementsStore::new();
    store.requirements.extend([epic, active, waiting]);
    let lease = lease_for("EPIC-20");
    let res = scope_fallback_pick(&store, &lease, None);
    assert!(res.is_none(), "should not double-pick under the same EPIC");
}

/// Path-glob / free-form scope can't resolve to a Requirement → None.
// trace:STORY-63 | ai:claude
#[test]
fn unresolved_scope_returns_none() {
    let store = RequirementsStore::new();
    let lease = lease_for("src/**/*.rs");
    assert!(scope_fallback_pick(&store, &lease, None).is_none());
}

/// Scope resolves but has no children → None (caller falls through
/// to the normal "queue empty" message + nudge).
// trace:STORY-63 | ai:claude
#[test]
fn scope_with_no_children_returns_none() {
    let epic = scope_root("EPIC-20", &[]);
    let mut store = RequirementsStore::new();
    store.requirements.push(epic);
    let lease = lease_for("EPIC-20");
    assert!(scope_fallback_pick(&store, &lease, None).is_none());
}

/// Children exist but none are Approved → None.
// trace:STORY-63 | ai:claude
#[test]
fn no_approved_children_returns_none() {
    let mut draft = child_of(Uuid::nil(), "STORY-DRAFT");
    draft.status = RequirementStatus::Draft;
    let epic = scope_root("EPIC-20", &[draft.id]);
    draft.relationships[0].target_id = epic.id;
    let mut store = RequirementsStore::new();
    store.requirements.extend([epic, draft]);
    let lease = lease_for("EPIC-20");
    assert!(scope_fallback_pick(&store, &lease, None).is_none());
}

/// Role scope filter on tags is honored — a candidate without all
/// the role's required tags is dropped.
// trace:STORY-63 | ai:claude
#[test]
fn role_scope_filter_drops_untagged_candidates() {
    let mut tagged = child_of(Uuid::nil(), "STORY-TAGGED");
    let mut untagged = child_of(Uuid::nil(), "STORY-PLAIN");
    tagged.priority = RequirementPriority::Low;
    untagged.priority = RequirementPriority::High;
    tagged.tags.insert("session".into());
    let epic = scope_root("EPIC-20", &[tagged.id, untagged.id]);
    for child in [&mut tagged, &mut untagged] {
        child.relationships[0].target_id = epic.id;
    }
    let mut store = RequirementsStore::new();
    store.requirements.extend([epic, tagged, untagged]);
    let lease = lease_for("EPIC-20");
    // Role requires the "session" tag — the untagged High-prio item
    // is dropped, the tagged Low-prio item wins.
    let role_scope = (vec!["session".to_string()], None);
    let res = scope_fallback_pick(&store, &lease, Some(&role_scope)).expect("expected a pick");
    assert_eq!(res.pick.spec_id.as_deref(), Some("STORY-TAGGED"));
}
