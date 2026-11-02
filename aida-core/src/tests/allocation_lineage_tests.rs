//! Lineage records: derived free space, the disjointness invariant, and
//! which blocks a clone may spend.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
use super::*;

const ROOT: &str = "aaaa";
const HUB: &str = "bbbb";
const LEAF: &str = "cccc";

fn pool(
    id: &str,
    typ: &str,
    start: u32,
    end: u32,
    parent: Option<&str>,
    holder: &str,
    home: &str,
) -> PoolRecord {
    PoolRecord {
        pool_id: id.into(),
        type_prefix: typ.into(),
        range_start: start,
        range_end: end,
        parent_pool_id: parent.map(Into::into),
        holder_node: holder.into(),
        home_ref: home.into(),
    }
}

fn block(node: &str, typ: &str, start: u32, end: u32, pool: Option<&str>) -> AgreedIdBlock {
    AgreedIdBlock {
        node_id: node.into(),
        owner: String::new(),
        hostname: String::new(),
        type_prefix: typ.into(),
        range_start: start,
        range_end: end,
        next: start,
        allocated_at: "2026-10-29T00:00:00Z".parse().unwrap(),
        pool_id: pool.map(Into::into),
    }
}

fn root() -> RootRecord {
    RootRecord {
        holder_node: ROOT.into(),
        epoch: 1,
    }
}

/// HOME root pool BUG 1-10000, a hub pool 2001-3000 homed on gitlab.
fn tree() -> Vec<PoolRecord> {
    vec![
        pool(
            "p-root-bug",
            "BUG",
            1,
            10_000,
            None,
            ROOT,
            "origin/aida-store",
        ),
        pool(
            "p-hub-bug",
            "BUG",
            2001,
            3000,
            Some("p-root-bug"),
            HUB,
            "gitlab/aida-store",
        ),
    ]
}

#[test]
fn free_space_is_parent_range_minus_children() {
    let pools = tree();
    let blocks = vec![
        block(ROOT, "BUG", 1, 100, Some("p-root-bug")),
        block(LEAF, "BUG", 2001, 2100, Some("p-hub-bug")),
        // A legacy block with no lineage takes nothing from any pool.
        block(LEAF, "BUG", 5000, 5100, None),
    ];
    assert_eq!(
        free_ranges(&pools[0], &pools, &blocks),
        vec![(101, 2000), (3001, 10_000)]
    );
    assert_eq!(free_ranges(&pools[1], &pools, &blocks), vec![(2101, 3000)]);
    let full = pool("p", "BUG", 1, 10, None, ROOT, "origin/aida-store");
    let children = vec![full.clone(), pool("c", "BUG", 1, 10, Some("p"), HUB, "x/y")];
    assert!(free_ranges(&full, &children, &[]).is_empty());
}

#[test]
fn a_consistent_tree_has_no_conflicts() {
    let blocks = vec![block(LEAF, "BUG", 2001, 2100, Some("p-hub-bug"))];
    assert_eq!(check_invariants(&tree(), &blocks, Some(&root())), vec![]);
}

#[test]
fn two_claims_union_merged_from_two_hubs_are_listed_not_resolved() {
    // Each hub's store took a claim from the same free range of one pool.
    let via_github = PoolRegistry { pools: tree() };
    let mut via_gitlab = via_github.clone();
    via_gitlab.pools.push(pool(
        "p-hub2-bug",
        "BUG",
        2500,
        3500,
        Some("p-root-bug"),
        LEAF,
        "gitlab/aida-store",
    ));
    let merged = via_github.union(&via_gitlab);
    // Identical records collapse; the new claim is kept for the check.
    assert_eq!(merged.pools.len(), 3);
    let blocks = vec![
        block(LEAF, "BUG", 2050, 2149, Some("p-hub-bug")),
        block(HUB, "BUG", 2100, 2199, Some("p-hub-bug")),
    ];
    let conflicts = check_invariants(&merged.pools, &blocks, Some(&root()));
    let overlaps: Vec<_> = conflicts
        .iter()
        .filter(|c| matches!(c, LineageConflict::Overlap { .. }))
        .collect();
    // hub vs hub2 under the root pool, and the two blocks under the hub.
    assert_eq!(overlaps.len(), 2, "{conflicts:?}");
}

#[test]
fn every_structural_violation_is_listed() {
    let pools = vec![
        pool(
            "p-root-bug",
            "BUG",
            1,
            1000,
            None,
            ROOT,
            "origin/aida-store",
        ),
        pool(
            "p-out",
            "BUG",
            900,
            1100,
            Some("p-root-bug"),
            HUB,
            "gitlab/aida-store",
        ),
        pool(
            "p-type",
            "FR",
            1,
            10,
            Some("p-root-bug"),
            HUB,
            "gitlab/aida-store",
        ),
        pool(
            "p-orphan",
            "BUG",
            1,
            10,
            Some("p-missing"),
            HUB,
            "gitlab/aida-store",
        ),
        pool("p-a", "BUG", 1, 10, Some("p-b"), HUB, "gitlab/aida-store"),
        pool("p-b", "BUG", 1, 10, Some("p-a"), HUB, "gitlab/aida-store"),
        pool(
            "p-rogue-root",
            "TASK",
            1,
            10,
            None,
            HUB,
            "gitlab/aida-store",
        ),
        pool("p-root-bug", "BUG", 1, 1000, None, ROOT, "other/aida-store"),
    ];
    let blocks = vec![
        block(LEAF, "*", 1, 10, Some("p-root-bug")),
        block(LEAF, "BUG", 10, 5, Some("p-root-bug")),
    ];
    let conflicts = check_invariants(&pools, &blocks, Some(&root()));
    let has = |pred: fn(&LineageConflict) -> bool| conflicts.iter().any(pred);
    assert!(has(|c| matches!(c, LineageConflict::DuplicatePoolId(_))));
    assert!(has(|c| matches!(c, LineageConflict::OutsideParent { .. })));
    assert!(has(|c| matches!(c, LineageConflict::TypeMismatch { .. })));
    assert!(has(|c| matches!(c, LineageConflict::UnknownParent { .. })));
    assert!(has(|c| matches!(c, LineageConflict::Cycle { .. })));
    assert!(has(|c| matches!(
        c,
        LineageConflict::RootPoolNotHeldByRoot { .. }
    )));
    assert!(has(|c| matches!(c, LineageConflict::WildcardType(_))));
    assert!(has(|c| matches!(c, LineageConflict::InvalidRange(_))));
    // A root pool with no enrolled root is never legitimate.
    assert!(check_invariants(&tree()[..1], &[], None)
        .iter()
        .any(|c| matches!(c, LineageConflict::RootPoolNotHeldByRoot { .. })));
}

#[test]
fn lineage_walks_to_the_root_and_refuses_cycles() {
    let pools = tree();
    let chain = lineage(&pools, "p-hub-bug").unwrap();
    assert_eq!(
        chain.iter().map(|p| p.pool_id.as_str()).collect::<Vec<_>>(),
        ["p-hub-bug", "p-root-bug"]
    );
    let cyclic = vec![
        pool("a", "BUG", 1, 2, Some("b"), HUB, "x/y"),
        pool("b", "BUG", 1, 2, Some("a"), HUB, "x/y"),
    ];
    assert!(lineage(&cyclic, "a").is_err());
    assert!(lineage(&pools, "missing").is_err());
}

fn view<'a>(
    pools: &'a [PoolRecord],
    blocks: &'a [AgreedIdBlock],
    root: Option<&'a RootRecord>,
    node: &'a str,
    role: AllocationRole,
    upstream: Option<&'a str>,
) -> LineageView<'a> {
    LineageView {
        pools,
        blocks,
        root,
        node,
        role,
        upstream,
        hub: None,
        hub_remotes: &[],
    }
}

#[test]
fn a_block_granted_through_the_declared_upstream_is_spendable() {
    let pools = tree();
    let blocks = vec![block(LEAF, "BUG", 2001, 2100, Some("p-hub-bug"))];
    let r = root();
    let v = view(
        &pools,
        &blocks,
        Some(&r),
        LEAF,
        AllocationRole::Node,
        Some("gitlab"),
    );
    assert_eq!(v.spendable_block("bug"), Ok(0));
    // The root spends blocks carved from its own pool without an upstream.
    let home = vec![block(ROOT, "BUG", 1, 100, Some("p-root-bug"))];
    let v = view(&pools, &home, Some(&r), ROOT, AllocationRole::Root, None);
    assert_eq!(v.spendable_block("BUG"), Ok(0));
}

#[test]
fn an_imported_block_without_lineage_is_never_spendable() {
    let pools = tree();
    let r = root();
    // No pool_id at all (a legacy or imported block).
    let legacy = vec![block(LEAF, "BUG", 2001, 2100, None)];
    let v = view(
        &pools,
        &legacy,
        Some(&r),
        LEAF,
        AllocationRole::Node,
        Some("gitlab"),
    );
    assert_eq!(
        v.spendable_block("BUG"),
        Err(Unusable::NoLineage("BUG".into()))
    );
    // A pool_id naming a pool this store does not record.
    let dangling = vec![block(LEAF, "BUG", 2001, 2100, Some("p-elsewhere"))];
    let v = view(
        &pools,
        &dangling,
        Some(&r),
        LEAF,
        AllocationRole::Node,
        Some("gitlab"),
    );
    assert!(matches!(
        v.spendable_block("BUG"),
        Err(Unusable::NoLineage(_)) | Err(Unusable::Conflict(_))
    ));
    // A chain that reaches a root pool held by a node that is not the root.
    let foreign = vec![pool(
        "p-other-root",
        "BUG",
        1,
        100,
        None,
        HUB,
        "elsewhere/aida-store",
    )];
    let blocks = vec![block(LEAF, "BUG", 1, 10, Some("p-other-root"))];
    let v = view(
        &foreign,
        &blocks,
        Some(&r),
        LEAF,
        AllocationRole::Node,
        Some("elsewhere"),
    );
    assert!(v.spendable_block("BUG").is_err());
    // Another node's block is never this node's.
    let theirs = vec![block(HUB, "BUG", 2001, 2100, Some("p-hub-bug"))];
    let v = view(
        &pools,
        &theirs,
        Some(&r),
        LEAF,
        AllocationRole::Node,
        Some("gitlab"),
    );
    assert_eq!(
        v.spendable_block("BUG"),
        Err(Unusable::NoBlock("BUG".into()))
    );
}

#[test]
fn a_mirror_or_other_remote_confers_nothing() {
    let pools = tree();
    let blocks = vec![block(LEAF, "BUG", 2001, 2100, Some("p-hub-bug"))];
    let r = root();
    let v = view(
        &pools,
        &blocks,
        Some(&r),
        LEAF,
        AllocationRole::Node,
        Some("origin"),
    );
    assert!(matches!(
        v.spendable_block("BUG"),
        Err(Unusable::WrongUpstream { .. })
    ));
    let v = view(&pools, &blocks, Some(&r), LEAF, AllocationRole::Node, None);
    assert_eq!(v.spendable_block("BUG"), Err(Unusable::NoUpstream));
}

#[test]
fn exhausted_and_conflicting_blocks_say_why() {
    let pools = tree();
    let r = root();
    let mut spent = block(LEAF, "BUG", 2001, 2100, Some("p-hub-bug"));
    spent.next = 2101;
    let blocks = vec![spent];
    let v = view(
        &pools,
        &blocks,
        Some(&r),
        LEAF,
        AllocationRole::Node,
        Some("gitlab"),
    );
    assert_eq!(
        v.spendable_block("BUG"),
        Err(Unusable::Exhausted("BUG".into()))
    );
    let overlapping = vec![
        block(LEAF, "BUG", 2001, 2100, Some("p-hub-bug")),
        block(HUB, "BUG", 2050, 2150, Some("p-hub-bug")),
    ];
    let v = view(
        &pools,
        &overlapping,
        Some(&r),
        LEAF,
        AllocationRole::Node,
        Some("gitlab"),
    );
    assert!(matches!(
        v.spendable_block("BUG"),
        Err(Unusable::Conflict(_))
    ));
}

#[test]
fn records_round_trip_and_refuse_unknown_fields() {
    let reg = PoolRegistry { pools: tree() };
    let text = serde_yaml::to_string(&reg).unwrap();
    assert_eq!(serde_yaml::from_str::<PoolRegistry>(&text).unwrap(), reg);
    assert!(serde_yaml::from_str::<PoolRecord>(
        "pool_id: p\ntype_prefix: BUG\nrange_start: 1\nrange_end: 2\nholder_node: a\nhome_ref: o/s\nranges: []\n"
    )
    .is_err());
    // A legacy block without pool_id still loads; an absent pool_id is not written.
    let legacy = block(LEAF, "BUG", 1, 2, None);
    assert!(!serde_yaml::to_string(&legacy).unwrap().contains("pool_id"));
}

#[test]
fn enrollment_refuses_a_second_root_and_is_idempotent_for_the_holder() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        enroll_preflight(dir.path(), ROOT).unwrap(),
        EnrollOutcome::Enrolled
    );
    std::fs::create_dir_all(dir.path().join("registry")).unwrap();
    std::fs::write(
        dir.path().join(ROOT_YAML_REL),
        serde_yaml::to_string(&root()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        enroll_preflight(dir.path(), ROOT).unwrap(),
        EnrollOutcome::AlreadyRoot
    );
    let err = enroll_preflight(dir.path(), LEAF).unwrap_err().to_string();
    assert!(err.contains("already the allocation root"), "{err}");
}

#[test]
fn enroll_root_commits_the_root_record_once() {
    let dir = tempfile::tempdir().unwrap();
    crate::git_ops::init(dir.path()).unwrap();
    // CI runners have no global git identity; the store commit needs one.
    crate::git_ops::configure_user(dir.path(), "Test", "test@example.com").unwrap();
    assert_eq!(
        enroll_root(dir.path(), ROOT).unwrap(),
        EnrollOutcome::Enrolled
    );
    assert_eq!(RootRecord::load(dir.path()).unwrap(), Some(root()));
    assert_eq!(
        enroll_root(dir.path(), ROOT).unwrap(),
        EnrollOutcome::AlreadyRoot
    );
    assert!(enroll_root(dir.path(), LEAF).is_err());
}

#[test]
fn cycle_message_tells_on_cycle_pools_from_pools_that_reach_one() {
    let pools = vec![
        pool("a", "BUG", 1, 2, Some("b"), HUB, "x/y"),
        pool("b", "BUG", 1, 2, Some("a"), HUB, "x/y"),
        pool("c", "BUG", 1, 2, Some("a"), HUB, "x/y"),
    ];
    let messages: Vec<String> = check_invariants(&pools, &[], Some(&root()))
        .iter()
        .filter(|c| matches!(c, LineageConflict::Cycle { .. }))
        .map(ToString::to_string)
        .collect();
    assert!(
        messages.contains(&"pool a is its own ancestor".to_string()),
        "{messages:?}"
    );
    assert!(
        messages.contains(&"pool b is its own ancestor".to_string()),
        "{messages:?}"
    );
    assert!(
        messages.contains(&"the lineage of pool c reaches a cycle at pool a".to_string()),
        "{messages:?}"
    );
}

// --- Single-hub mode: virtual hub pools, NotOnHub, materialization ---
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude

fn hub_view<'a>(
    blocks: &'a [AgreedIdBlock],
    root: Option<&'a RootRecord>,
    on_hub: &'a [AgreedIdBlock],
) -> LineageView<'a> {
    LineageView {
        pools: &[],
        blocks,
        root,
        node: LEAF,
        role: AllocationRole::Node,
        upstream: None,
        hub: Some(HubSpend {
            hub: "github",
            on_hub,
        }),
        hub_remotes: &[],
    }
}

#[test]
fn single_hub_block_on_the_hub_ref_is_spendable_without_a_root_or_upstream() {
    let blocks = vec![block(LEAF, "BUG", 1, 100, Some("hub:github:BUG"))];
    let v = hub_view(&blocks, None, &blocks);
    assert_eq!(v.spendable_block("BUG"), Ok(0));
    assert!(check_invariants(&[], &blocks, None).is_empty());
}

#[test]
fn single_hub_claim_not_on_the_hub_ref_is_not_spendable() {
    let blocks = vec![block(LEAF, "BUG", 1, 100, Some("hub:github:BUG"))];
    // Never pushed: the hub's ref has no such claim.
    let v = hub_view(&blocks, None, &[]);
    assert_eq!(
        v.spendable_block("BUG"),
        Err(Unusable::NotOnHub {
            type_prefix: "BUG".into(),
            hub: "github".into()
        })
    );
    // A claim for another hub, or with a different range, is not this one.
    // A hub this clone does not reach cannot vouch for its claim.
    let other = vec![block(LEAF, "BUG", 1, 100, Some("hub:gitlab:BUG"))];
    let v = hub_view(&other, None, &other);
    assert_eq!(
        v.spendable_block("BUG"),
        Err(Unusable::NotOnHub {
            type_prefix: "BUG".into(),
            hub: "gitlab".into()
        })
    );
    let moved = vec![block(LEAF, "BUG", 1, 99, Some("hub:github:BUG"))];
    let v = hub_view(&blocks, None, &moved);
    assert!(matches!(
        v.spendable_block("BUG"),
        Err(Unusable::NotOnHub { .. })
    ));
}

#[test]
fn hub_claims_still_list_overlaps_and_type_mismatches() {
    let blocks = vec![
        block(LEAF, "BUG", 1, 100, Some("hub:github:BUG")),
        block(HUB, "BUG", 50, 150, Some("hub:gitlab:BUG")),
        block(HUB, "TASK", 1, 10, Some("hub:github:BUG")),
    ];
    let conflicts = check_invariants(&[], &blocks, None);
    assert!(
        conflicts
            .iter()
            .any(|c| matches!(c, LineageConflict::Overlap { .. })),
        "{conflicts:?}"
    );
    assert!(
        conflicts
            .iter()
            .any(|c| matches!(c, LineageConflict::TypeMismatch { .. })),
        "{conflicts:?}"
    );
}

#[test]
fn with_a_root_an_unmaterialized_hub_pool_is_an_unknown_parent() {
    let blocks = vec![block(LEAF, "BUG", 1, 100, Some("hub:github:BUG"))];
    let r = root();
    let conflicts = check_invariants(&[], &blocks, Some(&r));
    assert!(matches!(
        conflicts.as_slice(),
        [LineageConflict::UnknownParent { .. }]
    ));
    // The hub context only applies while no root is recorded.
    let v = hub_view(&blocks, Some(&r), &blocks);
    assert!(v.spendable_block("BUG").is_err());
}

#[test]
fn materialized_pools_keep_every_block_lineage_and_free_space_excludes_them() {
    let blocks = vec![
        block(LEAF, "BUG", 1, 100, Some("hub:github:BUG")),
        block(HUB, "BUG", 101, 200, Some("hub:github:BUG")),
        block(LEAF, "TASK", 1, 100, Some("hub:github:TASK")),
        block(LEAF, "FR", 1, 100, None),
    ];
    let pools = pools_to_materialize(&[], &blocks, ROOT);
    assert_eq!(
        pools.iter().map(|p| p.pool_id.as_str()).collect::<Vec<_>>(),
        ["hub:github:BUG", "hub:github:TASK"]
    );
    assert!(pools
        .iter()
        .all(|p| p.parent_pool_id.is_none() && p.holder_node == ROOT && p.range_start == 1));
    assert_eq!(pools[0].home_ref, "github/aida-store");
    let r = root();
    assert!(check_invariants(&pools, &blocks, Some(&r)).is_empty());
    for b in blocks.iter().filter(|b| b.pool_id.is_some()) {
        let chain = lineage(&pools, b.pool_id.as_deref().unwrap()).unwrap();
        assert_eq!(chain.last().unwrap().holder_node, ROOT);
    }
    // A later grant carved from the root pool can never reuse an earlier ID.
    let free = free_ranges(&pools[0], &pools, &blocks);
    assert_eq!(free.first(), Some(&(201, u32::MAX)));
    // Already-recorded pools are not materialized again.
    assert!(pools_to_materialize(&pools, &blocks, ROOT).is_empty());
}

// AC5: after enroll-root, a non-root clone's block in a promoted hub pool
// stays spendable through whichever of its remotes reaches the hub, with or
// without a declared upstream; a clone that reaches no such hub is told so.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164.ac2f7ade | ai:claude
#[test]
fn a_promoted_hub_pool_is_spendable_through_the_remote_reaching_its_hub() {
    let blocks = vec![block(LEAF, "BUG", 1, 100, Some("hub:github:BUG"))];
    let pools = pools_to_materialize(&[], &blocks, ROOT);
    let r = root();
    let reaches = [HubReach {
        hub: "github".into(),
        remote: "origin".into(),
        on_hub: blocks.clone(),
    }];
    let mut v = view(&pools, &blocks, Some(&r), LEAF, AllocationRole::Node, None);
    v.hub_remotes = &reaches;
    assert_eq!(v.spendable_block("BUG"), Ok(0));
    // A declared upstream that is the hub's remote also passes...
    v.upstream = Some("origin");
    assert_eq!(v.spendable_block("BUG"), Ok(0));
    // ...one that is another remote does not.
    v.upstream = Some("gitlab");
    assert!(matches!(
        v.spendable_block("BUG"),
        Err(Unusable::WrongUpstream { .. })
    ));
    // No remote of this clone reaches the hub.
    let v = view(&pools, &blocks, Some(&r), LEAF, AllocationRole::Node, None);
    assert_eq!(
        v.spendable_block("BUG"),
        Err(Unusable::HubUnreached {
            type_prefix: "BUG".into(),
            hub: "github".into()
        })
    );
}

// AC3 after AC5: enrollment does not launder a claim the hub never ordered.
// A non-root clone's pre-enrollment hub claim that was never pushed
// (deferred or local-only), or that arrived only through an imported store,
// is not on the hub's store ref, so it stays unspendable once the root is
// enrolled and its pool is promoted.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164.ac00342e | ai:claude
#[test]
fn a_promoted_hub_pool_block_not_on_the_hub_ref_is_not_spendable() {
    let blocks = vec![block(LEAF, "BUG", 1, 100, Some("hub:github:BUG"))];
    let pools = pools_to_materialize(&[], &blocks, ROOT);
    let r = root();
    // The hub's ref holds another node's claim and an earlier, different
    // range of this node's, never this one.
    let reaches = [HubReach {
        hub: "github".into(),
        remote: "upstream".into(),
        on_hub: vec![
            block(HUB, "BUG", 101, 200, Some("hub:github:BUG")),
            block(LEAF, "BUG", 1, 50, Some("hub:github:BUG")),
        ],
    }];
    let mut v = view(&pools, &blocks, Some(&r), LEAF, AllocationRole::Node, None);
    v.hub_remotes = &reaches;
    let refused = Err(Unusable::NotOnHub {
        type_prefix: "BUG".into(),
        hub: "github".into(),
    });
    assert_eq!(v.spendable_block("BUG"), refused);
    // A declared upstream that is the hub's remote does not change that.
    v.upstream = Some("upstream");
    assert_eq!(v.spendable_block("BUG"), refused);
}

// AC4: multi-hub, no root: a clone reaching both declared hubs keeps a block
// stamped for the later-ordered hub, checked against that hub's own claims
// (not the first reached hub's).
// trace:STORY-B6B1278890774EF993D2EBC327862470-164.ac9679fe | ai:claude
#[test]
fn a_block_stamped_for_the_later_reached_hub_stays_spendable() {
    let blocks = vec![block(LEAF, "BUG", 1, 100, Some("hub:gitlab:BUG"))];
    let reaches = [
        HubReach {
            hub: "github".into(),
            remote: "origin".into(),
            on_hub: Vec::new(),
        },
        HubReach {
            hub: "gitlab".into(),
            remote: "lab".into(),
            on_hub: blocks.clone(),
        },
    ];
    let mut v = hub_view(&blocks, None, &[]);
    v.hub_remotes = &reaches;
    assert_eq!(v.spendable_block("BUG"), Ok(0));
    // Not on the later hub's ref: still refused, naming that hub.
    let unpushed = [
        reaches[0].clone(),
        HubReach {
            on_hub: Vec::new(),
            ..reaches[1].clone()
        },
    ];
    v.hub_remotes = &unpushed;
    assert_eq!(
        v.spendable_block("BUG"),
        Err(Unusable::NotOnHub {
            type_prefix: "BUG".into(),
            hub: "gitlab".into()
        })
    );
}

// STORY-170: the pool the root grants a second hub is disjoint from every
// block issued before the grant (and from the floor of issued numbers), a
// later grant to the same type starts above it, and the root's own pools
// stay conflict-free with the grant recorded.
// trace:STORY-B6B1278890774EF993D2EBC327862470-170 | ai:claude
#[test]
fn a_hub_grant_is_disjoint_from_every_pre_grant_block() {
    let blocks = vec![
        block(ROOT, "BUG", 1, 100, Some("hub:github:BUG")),
        block(LEAF, "BUG", 101, 200, Some("hub:github:BUG")),
        block(LEAF, "BUG", 5000, 5099, None),
    ];
    let mut pools = pools_to_materialize(&[], &blocks, ROOT);
    // The floor is the highest number any spec, block or counter holds.
    let grant = hub_child_pool("gitlab", &pools[0], &pools, &blocks, 5099, 1000).unwrap();
    assert_eq!(grant.pool_id, "hub:gitlab:BUG");
    assert_eq!(grant.parent_pool_id.as_deref(), Some("hub:github:BUG"));
    assert_eq!(grant.home_ref, "gitlab/aida-store");
    assert_eq!((grant.range_start, grant.range_end), (5100, 6099));
    for b in &blocks {
        assert!(
            b.range_end < grant.range_start || b.range_start > grant.range_end,
            "{b:?} overlaps {grant:?}"
        );
    }
    pools.push(grant);
    let r = root();
    assert!(check_invariants(&pools, &blocks, Some(&r)).is_empty());
    assert_eq!(granted_ceiling(&pools, "bug"), 6099);
    assert!(hub_grant(&pools, "gitlab", "BUG").is_some());
    assert!(
        hub_grant(&pools, "github", "BUG").is_none(),
        "a root pool is no grant"
    );
    // A second grant of the type starts above the first even at a low floor.
    let next = hub_child_pool("airgap", &pools[0], &pools, &blocks, 0, 10).unwrap();
    assert_eq!((next.range_start, next.range_end), (6100, 6109));
}

// STORY-170: a clone of the granted hub carves its blocks from the grant's
// free space and spends them through the remote reaching that hub, with its
// upstream set to it; the claim must be on that hub's store ref.
// trace:STORY-B6B1278890774EF993D2EBC327862470-170 | ai:claude
#[test]
fn a_clone_of_the_granted_hub_claims_and_spends_inside_the_grant() {
    let root_blocks = vec![block(ROOT, "BUG", 1, 100, Some("hub:github:BUG"))];
    let mut pools = pools_to_materialize(&[], &root_blocks, ROOT);
    let grant = hub_child_pool("gitlab", &pools[0], &pools, &root_blocks, 100, 1000).unwrap();
    pools.push(grant.clone());
    let (start, end) = carve_block(&grant, &pools, &root_blocks, 10).unwrap();
    assert_eq!((start, end), (101, 110));
    let mut blocks = root_blocks.clone();
    blocks.push(block(LEAF, "BUG", start, end, Some("hub:gitlab:BUG")));
    assert_eq!(carve_block(&grant, &pools, &blocks, 10), Some((111, 120)));
    let r = root();
    assert!(check_invariants(&pools, &blocks, Some(&r)).is_empty());
    let reaches = [HubReach {
        hub: "gitlab".into(),
        remote: "origin".into(),
        on_hub: blocks.clone(),
    }];
    let mut v = view(
        &pools,
        &blocks,
        Some(&r),
        LEAF,
        AllocationRole::Node,
        Some("origin"),
    );
    v.hub_remotes = &reaches;
    assert_eq!(v.spendable_block("BUG"), Ok(1));
    // An exhausted grant carves nothing.
    let tiny = PoolRecord {
        range_end: grant.range_start + 9,
        ..grant
    };
    assert_eq!(carve_block(&tiny, &pools, &blocks, 10), None);
}
