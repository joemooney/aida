//! `aida node hub add` planning helpers.
// trace:STORY-B6B1278890774EF993D2EBC327862470-170 | ai:claude
use super::*;

fn pool(id: &str, typ: &str, parent: Option<&str>, holder: &str, home: &str) -> PoolRecord {
    PoolRecord {
        pool_id: id.into(),
        type_prefix: typ.into(),
        range_start: 1,
        range_end: u32::MAX,
        parent_pool_id: parent.map(Into::into),
        holder_node: holder.into(),
        home_ref: home.into(),
    }
}

fn block(typ: &str) -> aida_core::AgreedIdBlock {
    aida_core::AgreedIdBlock {
        node_id: "n".into(),
        owner: String::new(),
        hostname: String::new(),
        type_prefix: typ.into(),
        range_start: 1,
        range_end: 10,
        next: 1,
        allocated_at: "2026-11-01T00:00:00Z".parse().unwrap(),
        pool_id: None,
    }
}

#[test]
fn types_in_use_are_every_block_and_pool_type_but_the_shared_scope() {
    let pools = [pool("hub:a:TASK", "TASK", None, "r", "a/aida-store")];
    let blocks = [block("bug"), block("BUG"), block("*"), block("STORY")];
    assert_eq!(types_in_use(&pools, &blocks), ["BUG", "STORY", "TASK"]);
}

#[test]
fn the_grant_parent_is_the_roots_pool_on_its_own_hub() {
    let pools = [
        pool("hub:other:BUG", "BUG", None, "r", "other/aida-store"),
        pool("hub:home:BUG", "BUG", None, "r", "home/aida-store"),
        pool(
            "hub:wk:BUG",
            "BUG",
            Some("hub:home:BUG"),
            "hub:wk",
            "wk/aida-store",
        ),
        pool("hub:x:BUG", "BUG", None, "not-root", "x/aida-store"),
    ];
    let p = root_pool(&pools, "bug", "r", Some("home")).unwrap();
    assert_eq!(p.pool_id, "hub:home:BUG");
    assert_eq!(
        root_pool(&pools, "BUG", "r", None).unwrap().pool_id,
        "hub:other:BUG"
    );
    assert!(root_pool(&pools, "TASK", "r", Some("home")).is_none());
}
