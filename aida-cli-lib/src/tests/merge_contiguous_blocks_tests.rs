use super::*;

fn block(
    node: &str,
    owner: &str,
    ty: &str,
    start: u32,
    end: u32,
    next: u32,
) -> aida_core::AgreedIdBlock {
    aida_core::AgreedIdBlock {
        node_id: node.to_string(),
        owner: owner.to_string(),
        hostname: "host".to_string(),
        type_prefix: ty.to_string(),
        range_start: start,
        range_end: end,
        next,
        allocated_at: chrono::Utc::now(),
    }
}

// Contiguous same-node/type/owner blocks fold into ONE row spanning the
// full start..end; the live frontier supplies next/remaining.
// trace:TASK-950 | ai:claude
#[test]
fn contiguous_blocks_merge_into_one_row() {
    // Mirrors the live store: node 1 BUG 317..916 across four exhausted
    // sub-blocks plus a live tail (next 632, remaining 285).
    let blocks = vec![
        block("1", "joe", "BUG", 317, 416, 417), // exhausted
        block("1", "joe", "BUG", 417, 516, 517), // exhausted
        block("1", "joe", "BUG", 517, 616, 617), // exhausted
        block("1", "joe", "BUG", 617, 916, 632), // live frontier
    ];
    let rows = merge_contiguous_blocks(&blocks);
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.range_start, 317);
    assert_eq!(r.range_end, 916);
    assert_eq!(r.next, 632);
    assert_eq!(r.remaining, 285);
}

// A gap in the range starts a new row.
// trace:TASK-950 | ai:claude
#[test]
fn non_contiguous_blocks_stay_separate() {
    // node 1 BUG 17..116 then 317..916 — gap 117..316 keeps them apart.
    let blocks = vec![
        block("1", "joe", "BUG", 17, 116, 117), // exhausted, isolated
        block("1", "joe", "BUG", 317, 416, 417),
        block("1", "joe", "BUG", 417, 916, 632),
    ];
    let rows = merge_contiguous_blocks(&blocks);
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].range_start, rows[0].range_end), (17, 116));
    assert_eq!((rows[1].range_start, rows[1].range_end), (317, 916));
}

// Different owner / type / node never merge even when ranges touch.
// trace:TASK-950 | ai:claude
#[test]
fn different_owner_type_or_node_do_not_merge() {
    let blocks = vec![
        // Same node+type, contiguous range, but DIFFERENT owner.
        block("1", "alice", "BUG", 1, 100, 50),
        block("1", "bob", "BUG", 101, 200, 150),
        // Same node+owner, contiguous range, but DIFFERENT type.
        block("2", "joe", "FR", 1, 100, 50),
        block("2", "joe", "TASK", 101, 200, 150),
        // Same owner+type, contiguous range, but DIFFERENT node.
        block("3", "joe", "EPIC", 1, 100, 50),
        block("4", "joe", "EPIC", 101, 200, 150),
    ];
    let rows = merge_contiguous_blocks(&blocks);
    // None of the three pairs may collapse.
    assert_eq!(rows.len(), 6);
}

// All sub-blocks exhausted => row is "full" (remaining 0); a live
// frontier in the run => remaining from that frontier.
// trace:TASK-950 | ai:claude
#[test]
fn full_vs_live_frontier_remaining() {
    // Every sub-block exhausted (next > range_end) -> full.
    let all_exhausted = vec![
        block("1", "joe", "TASK", 1, 100, 101),
        block("1", "joe", "TASK", 101, 200, 201),
    ];
    let rows = merge_contiguous_blocks(&all_exhausted);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].remaining, 0, "all-exhausted run renders as full");
    assert_eq!(rows[0].range_end, 200);

    // First sub-block live -> frontier is the first, not the last.
    let live_front = vec![
        block("1", "joe", "STORY", 1, 100, 40), // live: remaining 61
        block("1", "joe", "STORY", 101, 200, 201), // exhausted
    ];
    let rows = merge_contiguous_blocks(&live_front);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].next, 40);
    assert_eq!(rows[0].remaining, 61);
}
